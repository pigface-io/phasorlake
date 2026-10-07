-- ************************************************************ 
-- Title:	PUBLIC_DVD_BIDDUIDDETAILS_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS BIDDUIDDETAILS owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_BIDDUIDDETAILS_201501010000.csv  
APPEND INTO TABLE BIDDUIDDETAILS  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
DUID CHAR(10), 
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss", 
VERSIONNO FLOAT EXTERNAL, 
BIDTYPE CHAR(10), 
MAXCAPACITY FLOAT EXTERNAL, 
MINENABLEMENTLEVEL FLOAT EXTERNAL, 
MAXENABLEMENTLEVEL FLOAT EXTERNAL, 
MAXLOWERANGLE FLOAT EXTERNAL, 
MAXUPPERANGLE FLOAT EXTERNAL, 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss") 
-- End of Script 
