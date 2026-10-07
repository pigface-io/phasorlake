-- ************************************************************ 
-- Title:	PUBLIC_DVD_APEVENT_201510.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS APEVENT owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.24	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_APEVENT_201510010000.csv  
APPEND INTO TABLE APEVENT	
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
APEVENTID FLOAT EXTERNAL, 
EFFECTIVEFROMINTERVAL DATE "yyyy/mm/dd hh24:mi:ss", 
EFFECTIVETOINTERVAL DATE "yyyy/mm/dd hh24:mi:ss", 
REASON CHAR(2000), 
STARTAUTHORISEDBY CHAR(15), 
STARTAUTHORISEDDATE DATE "yyyy/mm/dd hh24:mi:ss", 
ENDAUTHORISEDBY CHAR(15), 
ENDAUTHORISEDDATE DATE "yyyy/mm/dd hh24:mi:ss", 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss") 
-- End of Script 
