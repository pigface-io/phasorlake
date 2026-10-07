-- ************************************************************ 
-- Title:	PUBLIC_DVD_RESIDUE_PUBLIC_DATA_201503.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS RESIDUE_PUBLIC_DATA owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_RESIDUE_PUBLIC_DATA_201503010000.csv  
APPEND INTO TABLE RESIDUE_PUBLIC_DATA  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
CONTRACTID CHAR(30), 
VERSIONNO FLOAT EXTERNAL, 
INTERCONNECTORID CHAR(10), 
FROMREGIONID CHAR(10), 
UNITSOFFERED FLOAT EXTERNAL, 
UNITSSOLD FLOAT EXTERNAL, 
CLEARINGPRICE FLOAT EXTERNAL, 
RESERVEPRICE FLOAT EXTERNAL, 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss") 
-- End of Script 
