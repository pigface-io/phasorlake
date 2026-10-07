-- ************************************************************ 
-- Title:	PUBLIC_DVD_DAYTRACK_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS DAYTRACK owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_DAYTRACK_201501010000.csv  
APPEND INTO TABLE DAYTRACK  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
REGIONID CHAR(10), 
EXANTERUNSTATUS CHAR(15), 
EXANTERUNNO FLOAT EXTERNAL, 
EXPOSTRUNSTATUS CHAR(15), 
EXPOSTRUNNO FLOAT EXTERNAL, 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss") 
-- End of Script 
