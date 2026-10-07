-- ************************************************************ 
-- Title:	PUBLIC_DVD_MARKETNOTICEDATA_201506.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS MARKETNOTICEDATA owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.24	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_MARKETNOTICEDATA_201506010000.csv  
APPEND INTO TABLE MARKETNOTICEDATA  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
NOTICEID FLOAT EXTERNAL, 
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss", 
TYPEID CHAR(25), 
NOTICETYPE CHAR(25), 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss", 
REASON CHAR(2000), 
EXTERNALREFERENCE CHAR(255)) 
-- End of Script 
